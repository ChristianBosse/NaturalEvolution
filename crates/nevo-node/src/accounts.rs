//! Accounts: the people who may add to this node, their roles and their signing keys.
//!
//! Kept in `accounts.sqlite` in the data folder, apart from the library: accounts belong to this
//! node and never sync. Readers need no account (D-008).
//!
//! Each person has their own Ed25519 key (D-038). Its secret is stored encrypted with
//! XChaCha20-Poly1305 under a key derived from their passphrase with Argon2id, so a copy of the
//! data folder doesn't hand out signing keys. A passphrase is checked by decrypting the key:
//! there is no separate password hash to keep in step, and sign-in costs one Argon2 run (D-050).
//!
//! Sessions are rows keyed by a hash of the cookie value. The secret key travels with the session,
//! encrypted under another value derived from the cookie, so the node can sign for a signed-in
//! person after a restart, while the database alone still can't (D-051).

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use nevo_format::{seal, KeyKind, KeyRecord, Keypair, PublicKey, Time, TimeSource};
use nevo_store::Store;
use rusqlite::{params, Connection, OptionalExtension};
use zeroize::Zeroizing;

pub const ACCOUNTS_FILE: &str = "accounts.sqlite";

/// Argon2id cost: 19 MiB, 2 passes, 1 lane (the OWASP minimum, and Argon2's own default). It
/// takes a fraction of a second on a Raspberry Pi 4 while making each guess at a stolen record
/// cost the same. Stored per account, so it can be raised later without breaking old accounts.
pub const KDF_MEMORY_KIB: u32 = 19 * 1024;
pub const KDF_PASSES: u32 = 2;
pub const KDF_LANES: u32 = 1;
/// Argon2 runs at the same time. Each takes 19 MiB, so many sign-ins at once can't exhaust a Pi.
const KDF_SLOTS: usize = 2;

/// Sessions end this long after sign-in.
pub const SESSION_SECONDS: i64 = 30 * 24 * 60 * 60;
pub const MIN_PASSPHRASE_CHARS: usize = 8;
const MAX_PASSPHRASE_BYTES: usize = 1024;
const MAX_NAME_CHARS: usize = 64;

/// Associated data for the encrypted secrets, so a ciphertext can't be moved to another use.
const ACCOUNT_AAD: &[u8] = b"nevo/account-key/1/";
const SESSION_AAD: &[u8] = b"nevo/session-key/1/";
/// BLAKE3 key-derivation contexts for the values derived from a session cookie.
const SESSION_ID_CONTEXT: &str = "nevo 2026-10 session id";
const SESSION_KEY_CONTEXT: &str = "nevo 2026-10 session key";
const CSRF_CONTEXT: &str = "nevo 2026-10 form token";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
-- `sealed` is the secret key encrypted under the Argon2id hash of the passphrase.
CREATE TABLE IF NOT EXISTS accounts (
    id       INTEGER PRIMARY KEY,
    name     TEXT NOT NULL UNIQUE COLLATE NOCASE,
    role     TEXT NOT NULL,
    key      BLOB NOT NULL UNIQUE,
    kdf_m    INTEGER NOT NULL,
    kdf_t    INTEGER NOT NULL,
    kdf_p    INTEGER NOT NULL,
    salt     BLOB NOT NULL,
    nonce    BLOB NOT NULL,
    sealed   BLOB NOT NULL,
    created  INTEGER NOT NULL,
    disabled INTEGER NOT NULL DEFAULT 0
);
-- `id` is derived from the cookie value, which is never stored. `wrapped` is the secret key
-- encrypted under another value derived from the cookie.
CREATE TABLE IF NOT EXISTS sessions (
    id      BLOB PRIMARY KEY,
    account INTEGER NOT NULL REFERENCES accounts (id),
    nonce   BLOB NOT NULL,
    wrapped BLOB NOT NULL,
    expires INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS sessions_account ON sessions (account);
";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Contributor,
    Admin,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Contributor => "contributor",
            Role::Admin => "admin",
        }
    }

    pub fn parse(text: &str) -> Option<Role> {
        match text {
            "contributor" => Some(Role::Contributor),
            "admin" => Some(Role::Admin),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Account {
    pub id: i64,
    pub name: String,
    pub role: Role,
    pub key: PublicKey,
    pub disabled: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AccountError {
    /// Wrong name or passphrase. Which one is not said.
    WrongPassphrase,
    Disabled,
    NameTaken,
    /// The node already has an admin (a second setup attempt).
    AlreadySetUp,
    /// The change would leave the node without an active admin.
    LastAdmin,
    /// Input that breaks a rule; the message is for the person.
    Invalid(String),
    Internal(String),
}

impl std::fmt::Display for AccountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            AccountError::WrongPassphrase => "Wrong name or passphrase.",
            AccountError::Disabled => "This account has been turned off by an admin.",
            AccountError::NameTaken => "That name is already used on this NEVO.",
            AccountError::AlreadySetUp => "This NEVO is already set up.",
            AccountError::LastAdmin => "This NEVO needs at least one active admin.",
            AccountError::Invalid(m) | AccountError::Internal(m) => m,
        })
    }
}

impl From<rusqlite::Error> for AccountError {
    fn from(e: rusqlite::Error) -> AccountError {
        AccountError::Internal(format!("accounts database: {e}"))
    }
}

impl From<nevo_store::StoreError> for AccountError {
    fn from(e: nevo_store::StoreError) -> AccountError {
        AccountError::Internal(format!("library: {e}"))
    }
}

pub type AccountResult<T> = Result<T, AccountError>;

/// A signed-in session, as found from its cookie.
pub struct Session {
    pub account: Account,
    token: Zeroizing<[u8; 32]>,
    nonce: Vec<u8>,
    wrapped: Vec<u8>,
}

impl Session {
    /// The form token for this session. Derived from the cookie value, so it needs no storage and
    /// can't be computed from the database.
    pub fn csrf_token(&self) -> String {
        hex(&blake3::derive_key(CSRF_CONTEXT, &*self.token))
    }

    /// The person's signing key, unwrapped for this request only.
    pub fn signing_key(&self) -> AccountResult<Keypair> {
        let key = Zeroizing::new(blake3::derive_key(SESSION_KEY_CONTEXT, &*self.token));
        let secret = decrypt(&key, &self.nonce, &self.wrapped, &session_aad(&self.token))
            .ok_or_else(|| AccountError::Internal("a session key didn't decrypt".into()))?;
        keypair_for(&secret, self.account.key)
    }
}

/// A new session: the cookie value to hand to the browser.
pub struct NewSession {
    pub account: Account,
    pub cookie: String,
}

pub struct Accounts {
    db: Mutex<Connection>,
    /// Cached: checked on every request while the node is not set up yet.
    has_admin: AtomicBool,
    /// Printed at startup until the setup wizard is done; /setup asks for it (D-052).
    setup_code: Mutex<Option<String>>,
    kdf: Limiter,
}

impl Accounts {
    /// Opens the accounts database, creating it if needed.
    pub fn open(path: impl AsRef<Path>) -> AccountResult<Accounts> {
        let conn = Connection::open(path)?;
        conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        conn.execute_batch("PRAGMA synchronous = FULL; PRAGMA foreign_keys = ON;")?;
        conn.execute_batch(SCHEMA)?;
        let accounts = Accounts {
            db: Mutex::new(conn),
            has_admin: AtomicBool::new(false),
            setup_code: Mutex::new(None),
            kdf: Limiter::new(KDF_SLOTS),
        };
        let has_admin = accounts.count_active_admins(&accounts.db())? > 0;
        accounts.has_admin.store(has_admin, Ordering::SeqCst);
        if !has_admin {
            *lock(&accounts.setup_code) = Some(new_setup_code()?);
        }
        Ok(accounts)
    }

    fn db(&self) -> MutexGuard<'_, Connection> {
        lock(&self.db)
    }

    /// Whether the setup wizard has been completed.
    pub fn has_admin(&self) -> bool {
        self.has_admin.load(Ordering::SeqCst)
    }

    /// The one-time setup code, while the node has no admin.
    pub fn setup_code(&self) -> Option<String> {
        lock(&self.setup_code).clone()
    }

    /// Whether `typed` is the setup code. Case, spaces and dashes don't matter.
    pub fn check_setup_code(&self, typed: &str) -> bool {
        let normalize = |s: &str| -> String {
            s.chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .map(|c| c.to_ascii_lowercase())
                .collect()
        };
        match lock(&self.setup_code).as_deref() {
            Some(code) => same_bytes(normalize(code).as_bytes(), normalize(typed).as_bytes()),
            None => false,
        }
    }

    pub fn node_name(&self) -> AccountResult<Option<String>> {
        Ok(self
            .db()
            .query_row("SELECT value FROM settings WHERE key = 'node_name'", [], |r| r.get(0))
            .optional()?)
    }

    pub fn set_node_name(&self, name: &str) -> AccountResult<()> {
        self.db().execute(
            "INSERT INTO settings (key, value) VALUES ('node_name', ?1)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [name],
        )?;
        Ok(())
    }

    /// Creates an account with a new key and publishes its key record through `node`'s log.
    /// With `first_admin`, it is refused if an admin already exists (two setup attempts at once).
    pub fn create(
        &self,
        store: &Store,
        node: &Keypair,
        name: &str,
        passphrase: &str,
        role: Role,
        first_admin: bool,
    ) -> AccountResult<(Account, Keypair)> {
        let name = check_name(name)?;
        check_passphrase(passphrase)?;
        if self.find_by_name(&name)?.is_some() {
            return Err(AccountError::NameTaken);
        }
        let key = Keypair::generate().map_err(|e| AccountError::Internal(e.to_string()))?;
        let salt = random::<16>()?;
        let nonce = random::<24>()?;
        let kek = self.derive(passphrase, &salt, KDF_MEMORY_KIB, KDF_PASSES, KDF_LANES)?;
        let sealed = encrypt(&kek, &nonce, &key.secret(), &account_aad(&key.public()))?;

        let id = {
            let mut db = self.db();
            let tx = db.transaction()?;
            if first_admin && self.count_active_admins(&tx)? > 0 {
                return Err(AccountError::AlreadySetUp);
            }
            let inserted = tx.execute(
                "INSERT INTO accounts (name, role, key, kdf_m, kdf_t, kdf_p, salt, nonce, sealed, created)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    name,
                    role.as_str(),
                    key.public().as_bytes(),
                    KDF_MEMORY_KIB,
                    KDF_PASSES,
                    KDF_LANES,
                    salt,
                    nonce,
                    sealed,
                    unix_seconds()
                ],
            );
            match inserted {
                Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == rusqlite::ErrorCode::ConstraintViolation => {
                    return Err(AccountError::NameTaken)
                }
                other => other?,
            };
            let id = tx.last_insert_rowid();
            tx.commit()?;
            id
        };
        // Published after the row is stored, so a refused name never leaves a key record behind.
        if let Err(e) = publish_key_record(store, node, &key, KeyKind::Person, &name) {
            let _ = self.db().execute("DELETE FROM accounts WHERE id = ?1", [id]);
            return Err(e);
        }
        if role == Role::Admin {
            self.has_admin.store(true, Ordering::SeqCst);
            *lock(&self.setup_code) = None;
        }
        let account = Account {
            id,
            name,
            role,
            key: key.public(),
            disabled: false,
        };
        Ok((account, key))
    }

    /// Checks a passphrase and returns the account's key.
    pub fn unlock(&self, name: &str, passphrase: &str) -> AccountResult<(Account, Keypair)> {
        let Some(stored) = self.find_by_name(name.trim())? else {
            // Do the same work as for a real account, so timing doesn't reveal which names exist.
            let _ = self.derive(passphrase, &[0; 16], KDF_MEMORY_KIB, KDF_PASSES, KDF_LANES);
            return Err(AccountError::WrongPassphrase);
        };
        let key = self.open_sealed(&stored, passphrase)?;
        if stored.account.disabled {
            return Err(AccountError::Disabled);
        }
        Ok((stored.account, key))
    }

    /// Checks the passphrase of a known account (re-entered before showing its secret key).
    pub fn unlock_id(&self, id: i64, passphrase: &str) -> AccountResult<Keypair> {
        let stored = self.find("id = ?1", id)?.ok_or(AccountError::WrongPassphrase)?;
        self.open_sealed(&stored, passphrase)
    }

    /// Encrypts the account's key under a new passphrase.
    pub fn change_passphrase(&self, id: i64, old: &str, new: &str) -> AccountResult<()> {
        check_passphrase(new)?;
        let key = self.unlock_id(id, old)?;
        let salt = random::<16>()?;
        let nonce = random::<24>()?;
        let kek = self.derive(new, &salt, KDF_MEMORY_KIB, KDF_PASSES, KDF_LANES)?;
        let sealed = encrypt(&kek, &nonce, &key.secret(), &account_aad(&key.public()))?;
        self.db().execute(
            "UPDATE accounts SET kdf_m = ?2, kdf_t = ?3, kdf_p = ?4, salt = ?5, nonce = ?6, sealed = ?7 WHERE id = ?1",
            params![id, KDF_MEMORY_KIB, KDF_PASSES, KDF_LANES, salt, nonce, sealed],
        )?;
        Ok(())
    }

    /// Signs in with a name and passphrase.
    pub fn sign_in(&self, name: &str, passphrase: &str) -> AccountResult<NewSession> {
        let (account, key) = self.unlock(name, passphrase)?;
        self.start_session(account, &key)
    }

    /// Starts a session for an account whose key is already unlocked.
    pub fn start_session(&self, account: Account, key: &Keypair) -> AccountResult<NewSession> {
        let token = Zeroizing::new(random::<32>()?);
        let wrap_key = Zeroizing::new(blake3::derive_key(SESSION_KEY_CONTEXT, &*token));
        let nonce = random::<24>()?;
        let wrapped = encrypt(&wrap_key, &nonce, &key.secret(), &session_aad(&token))?;
        let now = unix_seconds();
        let db = self.db();
        db.execute("DELETE FROM sessions WHERE expires <= ?1", [now])?;
        db.execute(
            "INSERT INTO sessions (id, account, nonce, wrapped, expires) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![session_id(&token), account.id, nonce, wrapped, now + SESSION_SECONDS],
        )?;
        Ok(NewSession {
            account,
            cookie: hex(&*token),
        })
    }

    /// The live session for a cookie value, if any.
    pub fn session(&self, cookie: &str) -> AccountResult<Option<Session>> {
        let Some(token) = unhex32(cookie) else {
            return Ok(None);
        };
        let token = Zeroizing::new(token);
        let row = self
            .db()
            .query_row(
                "SELECT s.account, s.nonce, s.wrapped FROM sessions s JOIN accounts a ON a.id = s.account
                 WHERE s.id = ?1 AND s.expires > ?2 AND a.disabled = 0",
                params![session_id(&token), unix_seconds()],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?, r.get::<_, Vec<u8>>(2)?)),
            )
            .optional()?;
        let Some((account_id, nonce, wrapped)) = row else {
            return Ok(None);
        };
        let Some(stored) = self.find("id = ?1", account_id)? else {
            return Ok(None);
        };
        Ok(Some(Session {
            account: stored.account,
            token,
            nonce,
            wrapped,
        }))
    }

    pub fn sign_out(&self, cookie: &str) -> AccountResult<()> {
        if let Some(token) = unhex32(cookie) {
            self.db().execute(
                "DELETE FROM sessions WHERE id = ?1",
                [session_id(&Zeroizing::new(token))],
            )?;
        }
        Ok(())
    }

    /// Every account, by name.
    pub fn list(&self) -> AccountResult<Vec<Account>> {
        let db = self.db();
        let mut stmt = db.prepare(&format!("{SELECT_ACCOUNT} ORDER BY name"))?;
        let rows = stmt.query_map([], row_to_stored)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?.map_err(AccountError::Internal)?.account);
        }
        Ok(out)
    }

    pub fn get(&self, id: i64) -> AccountResult<Option<Account>> {
        Ok(self.find("id = ?1", id)?.map(|s| s.account))
    }

    /// Whether a key belongs to an account on this node.
    pub fn is_local_key(&self, key: &PublicKey) -> AccountResult<bool> {
        Ok(self
            .db()
            .query_row("SELECT 1 FROM accounts WHERE key = ?1", [key.as_bytes()], |_| Ok(()))
            .optional()?
            .is_some())
    }

    /// Changes an account's role. The last active admin can't be demoted.
    pub fn set_role(&self, id: i64, role: Role) -> AccountResult<()> {
        self.change(id, |tx| {
            tx.execute(
                "UPDATE accounts SET role = ?2 WHERE id = ?1",
                params![id, role.as_str()],
            )?;
            Ok(())
        })
    }

    /// Turns an account off or on. Turning off ends its sessions. Keys and what they signed stay.
    pub fn set_disabled(&self, id: i64, disabled: bool) -> AccountResult<()> {
        self.change(id, |tx| {
            tx.execute(
                "UPDATE accounts SET disabled = ?2 WHERE id = ?1",
                params![id, i64::from(disabled)],
            )?;
            if disabled {
                tx.execute("DELETE FROM sessions WHERE account = ?1", [id])?;
            }
            Ok(())
        })
    }

    /// Applies a change to one account, refusing it if no active admin would be left.
    fn change(&self, id: i64, apply: impl FnOnce(&Connection) -> AccountResult<()>) -> AccountResult<()> {
        let mut db = self.db();
        let tx = db.transaction()?;
        let exists = tx
            .query_row("SELECT 1 FROM accounts WHERE id = ?1", [id], |_| Ok(()))
            .optional()?
            .is_some();
        if !exists {
            return Err(AccountError::Invalid("No such account.".into()));
        }
        apply(&tx)?;
        if self.count_active_admins(&tx)? == 0 {
            return Err(AccountError::LastAdmin);
        }
        tx.commit()?;
        Ok(())
    }

    fn count_active_admins(&self, conn: &Connection) -> AccountResult<i64> {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM accounts WHERE role = 'admin' AND disabled = 0",
            [],
            |r| r.get(0),
        )?)
    }

    fn find_by_name(&self, name: &str) -> AccountResult<Option<Stored>> {
        self.find("name = ?1", name)
    }

    fn find(&self, condition: &str, value: impl rusqlite::ToSql) -> AccountResult<Option<Stored>> {
        let db = self.db();
        let row = db
            .query_row(&format!("{SELECT_ACCOUNT} WHERE {condition}"), [value], row_to_stored)
            .optional()?;
        row.transpose().map_err(AccountError::Internal)
    }

    fn open_sealed(&self, stored: &Stored, passphrase: &str) -> AccountResult<Keypair> {
        let kek = self.derive(passphrase, &stored.salt, stored.kdf.0, stored.kdf.1, stored.kdf.2)?;
        let secret = decrypt(&kek, &stored.nonce, &stored.sealed, &account_aad(&stored.account.key))
            .ok_or(AccountError::WrongPassphrase)?;
        keypair_for(&secret, stored.account.key)
    }

    /// Runs Argon2id, at most [`KDF_SLOTS`] at a time.
    fn derive(&self, passphrase: &str, salt: &[u8], m: u32, t: u32, p: u32) -> AccountResult<Zeroizing<[u8; 32]>> {
        if passphrase.len() > MAX_PASSPHRASE_BYTES {
            return Err(AccountError::WrongPassphrase);
        }
        let _slot = self.kdf.acquire();
        derive_key(passphrase, salt, m, t, p)
    }
}

/// Argon2id of a passphrase: the key that encrypts the account's secret key.
pub fn derive_key(passphrase: &str, salt: &[u8], m: u32, t: u32, p: u32) -> AccountResult<Zeroizing<[u8; 32]>> {
    let params = Params::new(m, t, p, Some(32)).map_err(|e| AccountError::Internal(format!("argon2: {e}")))?;
    let mut out = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), salt, &mut *out)
        .map_err(|e| AccountError::Internal(format!("argon2: {e}")))?;
    Ok(out)
}

/// How long one Argon2id run with the default cost takes on this machine.
pub fn time_kdf() -> std::time::Duration {
    let start = Instant::now();
    let _ = derive_key(
        "correct horse battery staple",
        &[1; 16],
        KDF_MEMORY_KIB,
        KDF_PASSES,
        KDF_LANES,
    );
    start.elapsed()
}

/// Publishes a key record for `key`, signed by it, in `node`'s log.
pub fn publish_key_record(
    store: &Store,
    node: &Keypair,
    key: &Keypair,
    kind: KeyKind,
    name: &str,
) -> AccountResult<()> {
    let record = KeyRecord {
        key: key.public(),
        kind,
        name: name.to_string(),
        recovery: None,
        clock: store.author_clock(key.public())? + 1,
        time: Time {
            ms: unix_seconds() * 1000,
            src: TimeSource::Unknown,
        },
        extra: Default::default(),
    };
    let sealed = seal(&record, key).map_err(|e| AccountError::Invalid(format!("Please check the name: {e}")))?;
    store.publish(node, sealed.bytes())?;
    Ok(())
}

/// The text form of a secret key, as shown in the export QR code (D-053): a prefix, then the 32
/// bytes in uppercase base32, which QR codes store compactly (alphanumeric mode).
pub fn secret_text(key: &Keypair) -> Zeroizing<String> {
    let secret = Zeroizing::new(key.secret());
    let mut out = Zeroizing::new(String::from(SECRET_PREFIX));
    let (mut buffer, mut bits) = (0u32, 0u32);
    for &byte in secret.iter() {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(BASE32[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(BASE32[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

/// Reads a secret key written by [`secret_text`].
pub fn parse_secret_text(text: &str) -> Option<Keypair> {
    let body = text.trim().strip_prefix(SECRET_PREFIX)?;
    let mut secret = Zeroizing::new([0u8; 32]);
    let (mut buffer, mut bits, mut n) = (0u32, 0u32, 0usize);
    for c in body.bytes() {
        let value = BASE32.iter().position(|&b| b == c.to_ascii_uppercase())? as u32;
        buffer = (buffer << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            *secret.get_mut(n)? = (buffer >> bits) as u8;
            n += 1;
        }
    }
    (n == 32 && buffer & ((1 << bits) - 1) == 0).then(|| Keypair::from_secret(*secret))
}

const SECRET_PREFIX: &str = "NEVO-SECRET-1:";
const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

// ---------------------------------------------------------------------------------------------
// Helpers

struct Stored {
    account: Account,
    kdf: (u32, u32, u32),
    salt: Vec<u8>,
    nonce: Vec<u8>,
    sealed: Vec<u8>,
}

const SELECT_ACCOUNT: &str =
    "SELECT id, name, role, key, disabled, kdf_m, kdf_t, kdf_p, salt, nonce, sealed FROM accounts";

/// A row of [`SELECT_ACCOUNT`]. The inner error is for rows this code didn't write.
fn row_to_stored(r: &rusqlite::Row) -> rusqlite::Result<Result<Stored, String>> {
    let role: String = r.get(2)?;
    let key: Vec<u8> = r.get(3)?;
    let (Some(role), Ok(key)) = (Role::parse(&role), PublicKey::from_bytes(&key)) else {
        return Ok(Err("an account row is damaged".into()));
    };
    Ok(Ok(Stored {
        account: Account {
            id: r.get(0)?,
            name: r.get(1)?,
            role,
            key,
            disabled: r.get::<_, i64>(4)? != 0,
        },
        kdf: (r.get(5)?, r.get(6)?, r.get(7)?),
        salt: r.get(8)?,
        nonce: r.get(9)?,
        sealed: r.get(10)?,
    }))
}

fn check_name(name: &str) -> AccountResult<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AccountError::Invalid("Please give a name.".into()));
    }
    if name.chars().count() > MAX_NAME_CHARS || name.chars().any(char::is_control) {
        return Err(AccountError::Invalid(format!(
            "A name can have at most {MAX_NAME_CHARS} characters."
        )));
    }
    Ok(name.to_string())
}

pub fn check_passphrase(passphrase: &str) -> AccountResult<()> {
    if passphrase.chars().count() < MIN_PASSPHRASE_CHARS {
        return Err(AccountError::Invalid(format!(
            "The passphrase needs at least {MIN_PASSPHRASE_CHARS} characters. A few words are easier to remember."
        )));
    }
    if passphrase.len() > MAX_PASSPHRASE_BYTES {
        return Err(AccountError::Invalid("The passphrase is too long.".into()));
    }
    Ok(())
}

fn keypair_for(secret: &[u8], expected: PublicKey) -> AccountResult<Keypair> {
    let secret: Zeroizing<[u8; 32]> = Zeroizing::new(
        secret
            .try_into()
            .map_err(|_| AccountError::Internal("a stored key has the wrong length".into()))?,
    );
    let key = Keypair::from_secret(*secret);
    if key.public() != expected {
        return Err(AccountError::Internal("a stored key doesn't match its account".into()));
    }
    Ok(key)
}

fn account_aad(key: &PublicKey) -> Vec<u8> {
    [ACCOUNT_AAD, key.as_bytes().as_slice()].concat()
}

fn session_aad(token: &[u8; 32]) -> Vec<u8> {
    [SESSION_AAD, session_id(token).as_slice()].concat()
}

fn session_id(token: &[u8; 32]) -> [u8; 32] {
    blake3::derive_key(SESSION_ID_CONTEXT, token)
}

fn encrypt(key: &[u8; 32], nonce: &[u8; 24], plaintext: &[u8], aad: &[u8]) -> AccountResult<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|e| AccountError::Internal(e.to_string()))?;
    cipher
        .encrypt(&XNonce::from(*nonce), Payload { msg: plaintext, aad })
        .map_err(|_| AccountError::Internal("encryption failed".into()))
}

/// None if the key is wrong or the data was changed.
fn decrypt(key: &[u8; 32], nonce: &[u8], ciphertext: &[u8], aad: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    let cipher = XChaCha20Poly1305::new_from_slice(key).ok()?;
    let nonce: [u8; 24] = nonce.try_into().ok()?;
    cipher
        .decrypt(&XNonce::from(nonce), Payload { msg: ciphertext, aad })
        .ok()
        .map(Zeroizing::new)
}

fn random<const N: usize>() -> AccountResult<[u8; N]> {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).map_err(|e| AccountError::Internal(format!("random numbers: {e}")))?;
    Ok(bytes)
}

/// A random token as 64 hex characters.
pub fn random_token() -> AccountResult<String> {
    Ok(hex(&random::<32>()?))
}

/// 12 characters from an alphabet without look-alikes (no 0/o, 1/l/i): about 59 bits, shown in
/// groups of four so it is easy to type on a phone.
fn new_setup_code() -> AccountResult<String> {
    const ALPHABET: &[u8] = b"23456789abcdefghjkmnpqrstuvwxyz";
    let mut code = String::new();
    while code.len() < 14 {
        for byte in random::<32>()? {
            // Rejection sampling keeps every character equally likely.
            if usize::from(byte) < ALPHABET.len() * 8 && code.len() < 14 {
                if code.len() == 4 || code.len() == 9 {
                    code.push('-');
                }
                code.push(ALPHABET[usize::from(byte) % ALPHABET.len()] as char);
            }
        }
    }
    Ok(code)
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex32(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 || !text.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

/// Compares secrets in time that doesn't depend on where they differ.
pub fn same_bytes(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A counting semaphore for blocking threads.
struct Limiter {
    busy: Mutex<usize>,
    freed: Condvar,
    max: usize,
}

struct Slot<'a>(&'a Limiter);

impl Limiter {
    fn new(max: usize) -> Limiter {
        Limiter {
            busy: Mutex::new(0),
            freed: Condvar::new(),
            max,
        }
    }

    fn acquire(&self) -> Slot<'_> {
        let mut busy = lock(&self.busy);
        while *busy >= self.max {
            busy = self.freed.wait(busy).unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        *busy += 1;
        Slot(self)
    }
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        *lock(&self.0.busy) -= 1;
        self.0.freed.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accounts() -> (tempfile::TempDir, Accounts, Store, Keypair) {
        let dir = tempfile::tempdir().unwrap();
        let accounts = Accounts::open(dir.path().join(ACCOUNTS_FILE)).unwrap();
        let store = Store::open(dir.path().join("library")).unwrap();
        (dir, accounts, store, Keypair::from_secret([9; 32]))
    }

    #[test]
    fn secret_text_round_trips() {
        let key = Keypair::from_secret([42; 32]);
        let text = secret_text(&key);
        assert!(text.starts_with("NEVO-SECRET-1:"));
        assert_eq!(text.len(), 14 + 52);
        assert_eq!(parse_secret_text(&text).unwrap().public(), key.public());
        assert_eq!(
            parse_secret_text(&text.to_lowercase().replace("nevo-secret-1:", "NEVO-SECRET-1:"))
                .unwrap()
                .public(),
            key.public()
        );
        assert!(parse_secret_text("NEVO-SECRET-1:AAAA").is_none());
        assert!(parse_secret_text(&text[1..]).is_none());
    }

    #[test]
    fn setup_codes_are_typeable() {
        let code = new_setup_code().unwrap();
        assert_eq!(code.len(), 14, "{code}");
        assert_eq!(code.matches('-').count(), 2);
        assert!(!code.contains(['0', '1', 'o', 'l', 'i']));
    }

    #[test]
    fn keys_are_encrypted_and_sessions_survive_a_reopen() {
        let (dir, accounts, store, node) = accounts();
        assert!(!accounts.has_admin());
        assert!(accounts.setup_code().is_some());
        let (account, key) = accounts
            .create(&store, &node, "Amina", "a long passphrase", Role::Admin, true)
            .unwrap();
        assert!(accounts.has_admin());
        assert_eq!(accounts.setup_code(), None, "the setup code is used up");

        // The secret is nowhere in the file in clear form.
        drop(accounts);
        let file = std::fs::read(dir.path().join(ACCOUNTS_FILE)).unwrap();
        assert!(!file.windows(32).any(|w| w == key.secret()));

        let accounts = Accounts::open(dir.path().join(ACCOUNTS_FILE)).unwrap();
        assert!(accounts.has_admin());
        assert_eq!(
            accounts.sign_in("amina", "wrong passphrase").err(),
            Some(AccountError::WrongPassphrase)
        );
        assert_eq!(
            accounts.sign_in("nobody", "a long passphrase").err(),
            Some(AccountError::WrongPassphrase)
        );
        let session = accounts.sign_in("amina", "a long passphrase").unwrap();
        assert_eq!(session.account.id, account.id);

        drop(accounts);
        let accounts = Accounts::open(dir.path().join(ACCOUNTS_FILE)).unwrap();
        let found = accounts.session(&session.cookie).unwrap().unwrap();
        assert_eq!(found.signing_key().unwrap().public(), key.public());
        assert!(accounts.session(&"0".repeat(64)).unwrap().is_none());
        accounts.sign_out(&session.cookie).unwrap();
        assert!(accounts.session(&session.cookie).unwrap().is_none());
    }

    #[test]
    fn roles_and_the_last_admin() {
        let (_dir, accounts, store, node) = accounts();
        let (admin, _) = accounts
            .create(&store, &node, "Admin", "a long passphrase", Role::Admin, true)
            .unwrap();
        assert_eq!(
            accounts
                .create(&store, &node, "Second", "a long passphrase", Role::Admin, true)
                .err(),
            Some(AccountError::AlreadySetUp)
        );
        assert_eq!(
            accounts
                .create(&store, &node, "ADMIN", "a long passphrase", Role::Contributor, false)
                .err(),
            Some(AccountError::NameTaken)
        );
        assert!(matches!(
            accounts.create(&store, &node, "Short", "short", Role::Contributor, false),
            Err(AccountError::Invalid(_))
        ));
        let (writer, _) = accounts
            .create(&store, &node, "Writer", "a long passphrase", Role::Contributor, false)
            .unwrap();
        assert_eq!(
            accounts.set_role(admin.id, Role::Contributor),
            Err(AccountError::LastAdmin)
        );
        assert_eq!(accounts.set_disabled(admin.id, true), Err(AccountError::LastAdmin));
        accounts.set_role(writer.id, Role::Admin).unwrap();
        accounts.set_role(admin.id, Role::Contributor).unwrap();

        let session = accounts.sign_in("Admin", "a long passphrase").unwrap();
        accounts.set_disabled(admin.id, true).unwrap();
        assert!(
            accounts.session(&session.cookie).unwrap().is_none(),
            "turning off ends sessions"
        );
        assert_eq!(
            accounts.sign_in("Admin", "a long passphrase").err(),
            Some(AccountError::Disabled)
        );

        accounts
            .change_passphrase(writer.id, "a long passphrase", "another long one")
            .unwrap();
        assert!(accounts.sign_in("Writer", "a long passphrase").is_err());
        assert!(accounts.sign_in("Writer", "another long one").is_ok());
        // Every account published a key record.
        assert_eq!(store.log_position(node.public()).unwrap(), 2);
    }

    /// Prints how long sign-in takes: `cargo test --release -p nevo-node kdf_timing -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn kdf_timing() {
        let _ = time_kdf();
        let runs = 5;
        let total: std::time::Duration = (0..runs).map(|_| time_kdf()).sum();
        println!(
            "Argon2id m={KDF_MEMORY_KIB} KiB t={KDF_PASSES} p={KDF_LANES}: {:?} per run",
            total / runs
        );
    }
}
