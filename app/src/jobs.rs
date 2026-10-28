//! Jobs: one op each, run one at a time, saved to `<data_dir>/jobs/<id>.json` at every step.
//!
//! A job that may have sent a tx is settled by its position's state ([`settle`]), never assumed
//! lost: the wallet proves and sends in one call, so a crash there can leave a tx in flight with no
//! hash saved, and the testnet can drop a landed tx from its hash index (findings F6).

use std::{
    collections::BTreeMap,
    fs,
    io::Write as _,
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::PathBuf,
};

use anyhow::{Context as _, Result};
use bosphorus_client::lending::{Expect, Field};
use serde::{Deserialize, Serialize};

use crate::{ApiError, OpRequest};

/// After a window closes, how long the tx may still show up (clock skew, block lag).
pub const GRACE_MS: u64 = 60_000;
/// Ops without a window (supply_collateral, debt-free withdraw_collateral): how long after sending
/// they may still land, counted from the start of proving (~10 min) or from sending. The sequencer
/// includes or drops a tx within a few blocks of receiving it.
pub const NO_WINDOW_WAIT_MS: u64 = 30 * 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Queued,
    /// Reading state and checking the op with `core`.
    Preparing,
    /// A private tx being proven (~10 min on the testnet).
    Proving,
    /// A public tx being signed and sent.
    Sending,
    /// Sent: `tx` is known; waiting for it to land.
    Submitted,
    /// The app stopped while this job was proving or sending: watching the position until the tx
    /// can no longer land.
    Checking,
    Done,
    Failed,
    /// Nothing landed and nothing can: safe to submit again.
    Interrupted,
    /// Can't tell whether it landed (see `note`).
    Unknown,
}

impl State {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Interrupted | Self::Unknown)
    }
}

/// [`Expect`] as saved: numbers as strings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Expected {
    pub op: String,
    pub lending: String,
    pub position: String,
    pub field: String,
    pub from: String,
    pub to: String,
    pub amount: String,
    pub valid_until: Option<u64>,
    pub private: Vec<String>,
    pub hint: String,
}

impl From<&Expect> for Expected {
    fn from(e: &Expect) -> Self {
        Self {
            op: e.op.clone(),
            lending: e.lending.to_string(),
            position: e.position.to_string(),
            field: e.field.name().into(),
            from: e.from.to_string(),
            to: e.to.to_string(),
            amount: e.amount.to_string(),
            valid_until: e.valid_until,
            private: e.private.iter().map(ToString::to_string).collect(),
            hint: e.hint.clone(),
        }
    }
}

impl Expected {
    pub fn to_expect(&self) -> Result<Expect> {
        let id = |s: &str| s.parse().map_err(|e| anyhow::anyhow!("bad account id {s}: {e:?}"));
        Ok(Expect {
            op: self.op.clone(),
            lending: id(&self.lending)?,
            position: id(&self.position)?,
            field: Field::parse(&self.field)?,
            from: self.from.parse()?,
            to: self.to.parse()?,
            amount: self.amount.parse()?,
            valid_until: self.valid_until,
            private: self.private.iter().map(|s| id(s)).collect::<Result<_>>()?,
            hint: self.hint.clone(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub request: OpRequest,
    pub state: State,
    pub created_ms: u64,
    /// When `state` was entered.
    pub since_ms: u64,
    /// Saved before anything is sent.
    pub expect: Option<Expected>,
    pub tx: Option<String>,
    pub block: Option<u64>,
    pub sent_ms: Option<u64>,
    pub proving_ms: Option<u64>,
    /// Hex, one per private account, to decode the tx's notes after a restart. Only in the job file,
    /// which sits with the wallet (whose storage holds every key in the clear anyway).
    #[serde(default)]
    pub secrets: Vec<String>,
    /// Set by builds that save the tx's hash before handing it over: then a job stopped while
    /// proving or sending, with no hash saved, sent nothing.
    #[serde(default)]
    pub send_marked: bool,
    pub error: Option<ApiError>,
    pub note: Option<String>,
}

impl Job {
    /// The job as the API shows it: without the secrets.
    pub fn view(&self) -> serde_json::Value {
        let mut v = serde_json::to_value(self).expect("a job serialises");
        v.as_object_mut().expect("a job is an object").remove("secrets");
        v
    }
}

/// How an op that may have been sent stands. `value` is the position's field now (`None` when it
/// can't be read: a held position before its note is decoded, or an RPC error); `sent_ms` is when
/// it was (or may have been) sent. Never `NotApplied` while the tx could still apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Applied,
    Wait,
    NotApplied,
    Unknown(String),
}

pub fn settle(expect: &Expect, value: Option<u128>, now_ms: u64, sent_ms: u64) -> Verdict {
    let deadline = match expect.valid_until {
        Some(end) => end.saturating_add(GRACE_MS),
        None => sent_ms.saturating_add(NO_WINDOW_WAIT_MS),
    };
    match value {
        Some(v) if v == expect.to => Verdict::Applied,
        _ if now_ms < deadline => Verdict::Wait,
        Some(v) if v == expect.from => Verdict::NotApplied,
        Some(v) => Verdict::Unknown(format!(
            "the position's {} is {v}, neither {} (before) nor {} (after): something else moved it",
            expect.field.name(),
            expect.from,
            expect.to
        )),
        None if expect.held() => Verdict::Unknown("a held position: sync the wallet to see whether the tx landed".into()),
        None => Verdict::Wait,
    }
}

/// `<data_dir>/jobs/`, one file per job, private to the user.
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn open(dir: PathBuf) -> Result<(Self, BTreeMap<String, Job>)> {
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
        let mut jobs = BTreeMap::new();
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "json") {
                let job: Job = serde_json::from_slice(&fs::read(&path)?).with_context(|| format!("reading {}", path.display()))?;
                jobs.insert(job.id.clone(), job);
            }
        }
        Ok((Self { dir }, jobs))
    }

    /// Written to a temporary file and renamed, so a crash never leaves half a job.
    pub fn save(&self, job: &Job) -> Result<()> {
        let path = self.dir.join(format!("{}.json", job.id));
        let tmp = path.with_extension("json.tmp");
        let mut file = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        file.write_all(&serde_json::to_vec_pretty(job)?)?;
        file.sync_all()?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expect(valid_until: Option<u64>, held: bool) -> Expect {
        let position = lee::AccountId::new([7; 32]);
        Expect {
            op: "borrow".into(),
            lending: lee::AccountId::new([1; 32]),
            position,
            field: Field::BorrowShares,
            from: 100,
            to: 150,
            amount: 50,
            valid_until,
            private: if held { vec![position] } else { vec![] },
            hint: String::new(),
        }
    }

    /// A tx that may still land is never reported as safe to resubmit (a resubmitted borrow would
    /// borrow twice).
    #[test]
    fn never_not_applied_while_the_tx_can_land() {
        let e = expect(Some(1_000_000), false);
        assert_eq!(settle(&e, Some(100), 1_000_000 + GRACE_MS - 1, 0), Verdict::Wait);
        assert_eq!(settle(&e, None, 1_000_000 + GRACE_MS - 1, 0), Verdict::Wait);
        assert_eq!(settle(&e, Some(100), 1_000_000 + GRACE_MS, 0), Verdict::NotApplied);
        let no_window = expect(None, false);
        assert_eq!(settle(&no_window, Some(100), 5_000 + NO_WINDOW_WAIT_MS - 1, 5_000), Verdict::Wait);
        assert_eq!(settle(&no_window, Some(100), 5_000 + NO_WINDOW_WAIT_MS, 5_000), Verdict::NotApplied);
    }

    #[test]
    fn applied_as_soon_as_the_position_shows_it() {
        assert_eq!(settle(&expect(Some(1_000_000), false), Some(150), 0, 0), Verdict::Applied);
    }

    /// Unreadable state never turns into "nothing landed": a held position stays unknown, and an
    /// RPC error keeps waiting.
    #[test]
    fn unreadable_is_never_not_applied() {
        assert!(matches!(settle(&expect(Some(0), true), None, u64::MAX, 0), Verdict::Unknown(_)));
        assert_eq!(settle(&expect(Some(0), false), None, u64::MAX, 0), Verdict::Wait);
        assert!(matches!(settle(&expect(Some(0), false), Some(120), u64::MAX, 0), Verdict::Unknown(_)));
    }
}
