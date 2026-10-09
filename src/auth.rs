use std::{
    net::IpAddr,
    path::Path,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use dashmap::DashMap;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use crate::config::Config;

pub const COOKIE_NAME: &str = "ttm_session";
const MAX_FAILURES: u32 = 5;
const LOCKOUT: Duration = Duration::from_secs(60);
/// `ttm hash-password` refuses anything shorter.
pub const MIN_PASSWORD_LEN: usize = 12;
/// Logins over a tunnel whose client we cannot tell apart share one slow lane.
const SHARED_GAP: Duration = Duration::from_secs(2);

type HmacSha256 = Hmac<Sha256>;

pub enum Credential {
    Hash(String),
    Plain(String),
}

pub struct Auth {
    credential: Credential,
    key: Vec<u8>,
    ttl: Duration,
    failures: DashMap<IpAddr, (u32, Instant)>,
    /// When the last login attempt from an unidentifiable client began.
    shared_last: std::sync::Mutex<Option<Instant>>,
}

pub fn hash_password(pw: &str) -> Result<String> {
    Argon2::default()
        .hash_password(pw.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| anyhow::anyhow!("hashing password: {e}"))
}

impl Auth {
    pub fn new(cfg: &Config, data_dir: &Path) -> Result<Self> {
        let credential = match (&cfg.password_hash, &cfg.password) {
            (Some(h), _) => Credential::Hash(h.clone()),
            (None, Some(p)) => Credential::Plain(p.clone()),
            (None, None) => anyhow::bail!("no password configured"),
        };
        Ok(Self {
            credential,
            key: load_or_create_key(&data_dir.join("cookie.key"))?,
            ttl: Duration::from_secs(cfg.session_ttl_hours * 3600),
            failures: DashMap::new(),
            shared_last: std::sync::Mutex::new(None),
        })
    }

    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// Returns false while `ip` is locked out from too many failed logins.
    pub fn may_attempt(&self, ip: IpAddr) -> bool {
        match self.failures.get(&ip) {
            Some(e) => e.0 < MAX_FAILURES || e.1.elapsed() > LOCKOUT,
            None => true,
        }
    }

    /// For logins that arrive through a tunnel without the client's address: every one of them
    /// looks like 127.0.0.1, so a per-address lockout would let one attacker lock everyone out.
    /// Allow one attempt per `SHARED_GAP` across all of them instead.
    pub fn may_attempt_shared(&self) -> bool {
        let mut last = self.shared_last.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let ok = last.is_none_or(|t| t.elapsed() >= SHARED_GAP);
        if ok {
            *last = Some(Instant::now());
        }
        ok
    }

    pub fn record(&self, ip: IpAddr, ok: bool) {
        if ok {
            self.failures.remove(&ip);
            return;
        }
        let mut e = self.failures.entry(ip).or_insert((0, Instant::now()));
        if e.1.elapsed() > LOCKOUT {
            e.0 = 0;
        }
        e.0 += 1;
        e.1 = Instant::now();
    }

    pub fn verify_password(&self, attempt: &str) -> bool {
        match &self.credential {
            Credential::Plain(p) => p.as_bytes().ct_eq(attempt.as_bytes()).into(),
            Credential::Hash(h) => PasswordHash::new(h)
                .map(|parsed| {
                    Argon2::default()
                        .verify_password(attempt.as_bytes(), &parsed)
                        .is_ok()
                })
                .unwrap_or(false),
        }
    }

    /// Token format: `<expiry-unix>.<b64 hmac>`.
    pub fn issue_token(&self) -> String {
        let exp = now() + self.ttl.as_secs();
        format!("{exp}.{}", B64.encode(self.mac(exp)))
    }

    pub fn check_token(&self, token: &str) -> bool {
        let Some((exp, sig)) = token.split_once('.') else {
            return false;
        };
        let (Ok(exp), Ok(sig)) = (exp.parse::<u64>(), B64.decode(sig)) else {
            return false;
        };
        exp > now() && bool::from(sig.ct_eq(&self.mac(exp)))
    }

    fn mac(&self, exp: u64) -> Vec<u8> {
        let mut m = HmacSha256::new_from_slice(&self.key).expect("HMAC accepts any key length");
        m.update(exp.to_string().as_bytes());
        m.finalize().into_bytes().to_vec()
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn load_or_create_key(path: &Path) -> Result<Vec<u8>> {
    if let Ok(k) = std::fs::read(path)
        && k.len() >= 32
    {
        return Ok(k);
    }
    let mut key = vec![0u8; 32];
    rand::fill(&mut key[..]);
    std::fs::write(path, &key).with_context(|| format!("writing {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth(pw: Credential) -> Auth {
        Auth {
            credential: pw,
            key: vec![7; 32],
            ttl: Duration::from_secs(60),
            failures: DashMap::new(),
            shared_last: std::sync::Mutex::new(None),
        }
    }

    #[test]
    fn hash_roundtrip() {
        let h = hash_password("hunter2").unwrap();
        let a = auth(Credential::Hash(h));
        assert!(a.verify_password("hunter2"));
        assert!(!a.verify_password("nope"));
    }

    #[test]
    fn token_roundtrip_and_tamper() {
        let a = auth(Credential::Plain("x".into()));
        let t = a.issue_token();
        assert!(a.check_token(&t));
        assert!(!a.check_token(&format!("9{t}")));
        assert!(!a.check_token("garbage"));
    }

    #[test]
    fn lockout_after_failures() {
        let a = auth(Credential::Plain("x".into()));
        let ip: IpAddr = [1, 2, 3, 4].into();
        for _ in 0..MAX_FAILURES {
            a.record(ip, false);
        }
        assert!(!a.may_attempt(ip));
        a.record(ip, true);
        assert!(a.may_attempt(ip));
    }

    #[test]
    fn anonymous_logins_share_one_slow_lane_and_never_lock_out() {
        let a = auth(Credential::Plain("x".into()));
        assert!(a.may_attempt_shared());
        assert!(!a.may_attempt_shared(), "a second attempt right away waits");
        *a.shared_last.lock().unwrap() = Some(Instant::now() - SHARED_GAP);
        assert!(a.may_attempt_shared());
        // The per-address lockout is untouched by any of this.
        assert!(a.may_attempt([127, 0, 0, 1].into()));
    }
}
